-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#NETWORK_STATICRATING#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS NETWORK_STATICRATING owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#NETWORK_STATICRATING#FILE01#202608010000.CSV
APPEND INTO TABLE NETWORK_STATICRATING
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SUBSTATIONID CHAR(30),
EQUIPMENTTYPE CHAR(10),
EQUIPMENTID CHAR(30),
RATINGLEVEL CHAR(10),
APPLICATIONID CHAR(20),
VALIDFROM DATE "YYYY/MM/DD HH24:MI:SS",
VALIDTO DATE "YYYY/MM/DD HH24:MI:SS",
RATINGVALUE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
