-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#NETWORK_OUTAGEDETAIL#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS NETWORK_OUTAGEDETAIL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#NETWORK_OUTAGEDETAIL#FILE01#202608010000.CSV
APPEND INTO TABLE NETWORK_OUTAGEDETAIL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
OUTAGEID FLOAT EXTERNAL,
SUBSTATIONID CHAR(30),
EQUIPMENTTYPE CHAR(10),
EQUIPMENTID CHAR(30),
STARTTIME DATE "YYYY/MM/DD HH24:MI:SS",
ENDTIME DATE "YYYY/MM/DD HH24:MI:SS",
SUBMITTEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
OUTAGESTATUSCODE CHAR(10),
RESUBMITREASON CHAR(50),
RESUBMITOUTAGEID FLOAT EXTERNAL,
RECALLTIMEDAY FLOAT EXTERNAL,
RECALLTIMENIGHT FLOAT EXTERNAL,
LASTCHANGED TIMESTAMP(3),
REASON CHAR(100),
ISSECONDARY FLOAT EXTERNAL,
ACTUAL_STARTTIME DATE "YYYY/MM/DD HH24:MI:SS",
ACTUAL_ENDTIME DATE "YYYY/MM/DD HH24:MI:SS",
COMPANYREFCODE CHAR(20),
ELEMENTID FLOAT EXTERNAL)
-- End of Script
