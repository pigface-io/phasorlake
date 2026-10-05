-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PMS_GROUPSERVICE#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PMS_GROUPSERVICE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PMS_GROUPSERVICE#FILE01#202608010000.CSV
APPEND INTO TABLE PMS_GROUPSERVICE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
GROUPSERVICEID FLOAT EXTERNAL,
GROUPID FLOAT EXTERNAL,
VERSIONFROM TIMESTAMP(3),
VERSIONTO TIMESTAMP(3),
STARTDATE TIMESTAMP(3),
ENDDATE TIMESTAMP(3),
MARKET CHAR(50),
SERVICETYPE CHAR(50),
ENTITYTYPE CHAR(50),
ENTITYID CHAR(50),
MRC FLOAT EXTERNAL,
MRCREASON CHAR(500),
MAXIMUMRAMPRATEPERMIN FLOAT EXTERNAL,
REGION CHAR(20),
APPROVEDDATE TIMESTAMP(3),
LASTCHANGED TIMESTAMP(3))
-- End of Script
