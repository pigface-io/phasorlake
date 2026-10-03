-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#GENCONSETINVOKE#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GENCONSETINVOKE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#GENCONSETINVOKE#FILE01#202608010000.CSV
APPEND INTO TABLE GENCONSETINVOKE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
STARTDATE DATE "YYYY/MM/DD HH24:MI:SS",
STARTPERIOD FLOAT EXTERNAL,
GENCONSETID CHAR(20),
ENDDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENDPERIOD FLOAT EXTERNAL,
STARTAUTHORISEDBY CHAR(15),
ENDAUTHORISEDBY CHAR(15),
INTERVENTION CHAR(1),
ASCONSTRAINTTYPE CHAR(10),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
STARTINTERVALDATETIME DATE "YYYY/MM/DD HH24:MI:SS",
ENDINTERVALDATETIME DATE "YYYY/MM/DD HH24:MI:SS",
SYSTEMNORMAL CHAR(1),
INVOCATION_ID FLOAT EXTERNAL)
-- End of Script
